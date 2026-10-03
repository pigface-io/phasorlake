-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#DISPATCH_INTERCONNECTION#FILE01#202608.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS DISPATCH_INTERCONNECTION owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#DISPATCH_INTERCONNECTION#FILE01#202608010000.CSV
APPEND INTO TABLE DISPATCH_INTERCONNECTION
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
SETTLEMENTDATE DATE "YYYY/MM/DD HH24:MI:SS",
RUNNO FLOAT EXTERNAL,
INTERVENTION FLOAT EXTERNAL,
FROM_REGIONID CHAR(20),
TO_REGIONID CHAR(20),
DISPATCHINTERVAL FLOAT EXTERNAL,
IRLF FLOAT EXTERNAL,
MWFLOW FLOAT EXTERNAL,
METEREDMWFLOW FLOAT EXTERNAL,
FROM_REGION_MW_LOSSES FLOAT EXTERNAL,
TO_REGION_MW_LOSSES FLOAT EXTERNAL,
LASTCHANGED DATE "YYYY/MM/DD HH24:MI:SS")
-- End of Script
