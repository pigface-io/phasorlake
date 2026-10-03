-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#RESIDUE_PUBLIC_DATA#FILE01#202608.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS RESIDUE_PUBLIC_DATA owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#RESIDUE_PUBLIC_DATA#FILE01#202608010000.CSV
APPEND INTO TABLE RESIDUE_PUBLIC_DATA
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
CONTRACTID CHAR(30),
VERSIONNO FLOAT EXTERNAL,
INTERCONNECTORID CHAR(10),
FROMREGIONID CHAR(10),
UNITSOFFERED FLOAT EXTERNAL,
UNITSSOLD FLOAT EXTERNAL,
CLEARINGPRICE FLOAT EXTERNAL,
RESERVEPRICE FLOAT EXTERNAL,
LASTCHANGED DATE "YYYY/MM/DD HH24:MI:SS")
-- End of Script
