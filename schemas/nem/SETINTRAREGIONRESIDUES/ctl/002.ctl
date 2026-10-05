-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#SETINTRAREGIONRESIDUES#FILE01#202608.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS SETINTRAREGIONRESIDUES owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_ARCHIVE#SETINTRAREGIONRESIDUES#FILE01#202608010000.CSV
APPEND INTO TABLE SETINTRAREGIONRESIDUES
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
PERIODID FLOAT EXTERNAL,
REGIONID CHAR(10),
EP FLOAT EXTERNAL,
EC FLOAT EXTERNAL,
RRP FLOAT EXTERNAL,
EXP FLOAT EXTERNAL,
IRSS FLOAT EXTERNAL,
LASTCHANGED DATE "YYYY/MM/DD HH24:MI:SS",
ACE_AMOUNT FLOAT EXTERNAL,
ASOE_AMOUNT FLOAT EXTERNAL)
-- End of Script
