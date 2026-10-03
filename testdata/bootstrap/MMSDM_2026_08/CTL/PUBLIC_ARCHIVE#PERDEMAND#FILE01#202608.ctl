-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#PERDEMAND#FILE01#202608.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS PERDEMAND owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#PERDEMAND#FILE01#202608010000.CSV
APPEND INTO TABLE PERDEMAND
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
EFFECTIVEDATE DATE "YYYY/MM/DD HH24:MI:SS",
SETTLEMENTDATE DATE "YYYY/MM/DD HH24:MI:SS",
REGIONID CHAR(10),
OFFERDATE DATE "YYYY/MM/DD HH24:MI:SS",
PERIODID FLOAT EXTERNAL,
VERSIONNO FLOAT EXTERNAL,
RESDEMAND FLOAT EXTERNAL,
DEMAND90PROBABILITY FLOAT EXTERNAL,
DEMAND10PROBABILITY FLOAT EXTERNAL,
LASTCHANGED DATE "YYYY/MM/DD HH24:MI:SS",
MR_SCHEDULE FLOAT EXTERNAL)
-- End of Script
